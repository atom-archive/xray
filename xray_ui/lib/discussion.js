const React = require("react");
const PropTypes = require("prop-types");
const { styled } = require("styletron-react");
const TextareaAutosize = require("react-autosize-textarea").default;
const $ = React.createElement;

const Root = styled("div", {
  width: "100%",
  height: "100%",
  padding: "5px",
  margin: 0,
  display: "flex",
  flexDirection: "column",
  boxSizing: "border-box",
  backgroundColor: "rgb(234, 234, 235)"
});

const Messages = styled("div", {
  flex: 1,
  background: "white",
  marginBottom: "5px",
  overflowY: "scroll",
  "::-webkit-scrollbar": {
    width: "5px",
  },
  "::-webkit-scrollbar-thumb": {
    borderRadius: "5px",
    background: "rgba(150, 150, 150, .33)"
  }
});

const Message = styled("div", {
  padding: "5px",
  fontFamily: "Helvetica Neue",
  cursor: "default",
  ":hover": {
    background: "rgba(31, 150, 255, 0.3)"
  }
});

const Avatar = styled("div", ({ color }) => {
  const { r, g, b, a } = color;
  return {
    backgroundColor: `rgba(${r}, ${g}, ${b}, ${a})`,
    display: "inline-block",
    width: "16px",
    height: "16px",
    position: "relative",
    top: "2px",
    marginRight: "4px"
  };
});

const TextArea = styled(TextareaAutosize, {
  padding: "5px",
  fontSize: "14px",
  resize: "none"
});

class Discussion extends React.Component {
  constructor() {
    super();
    this.handleKeyDown = this.handleKeyDown.bind(this);
  }

  componentDidMount() {
    this.scrollToBottom();
  }

  componentDidUpdate() {
    this.scrollToBottom();
  }

  scrollToBottom() {
    this.messages.scrollTop =
      this.messages.scrollHeight - this.messages.offsetHeight;
  }

  render() {
    const { avatarColors } = this.context.theme;
    return $(
      Root,
      null,
      $(
        Messages,
        {
          $ref: messages => {
            this.messages = messages;
          }
        },
        this.props.messages.map(message => {
          const avatarColor =
            avatarColors[message.user_id % avatarColors.length];
          return $(
            Message,
            null,
            $(Avatar, { color: avatarColor }),
            message.text
          );
        })
      ),
      $(TextArea, {
        maxRows: 6,
        innerRef: textArea => (this.textArea = textArea),
        placeholder: "Message collaborators",
        onKeyDown: this.handleKeyDown
      })
    );
  }

  handleKeyDown(event) {
    if (event.key === "Enter") {
      if (this.textArea.value.length > 0) {
        this.props.dispatch({
          type: "Send",
          text: this.textArea.value
        });
        this.textArea.value = "";
      }
      event.preventDefault();
    }
  }
}

Discussion.contextTypes = {
  theme: PropTypes.object
};

module.exports = Discussion;
